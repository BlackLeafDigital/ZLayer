# BridgeNetworkDetails

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **time.Time** | Creation timestamp (UTC, RFC 3339). | 
**Driver** | Pointer to [**BridgeNetworkDriver**](BridgeNetworkDriver.md) | Driver backing the network (bridge vs. overlay). | [optional] 
**Id** | **string** | Opaque server-generated identifier (UUID v4). | 
**Internal** | Pointer to **bool** | If true, containers attached to this network cannot reach the outside world — only other containers on the same network. | [optional] 
**Labels** | Pointer to **map[string]string** | Arbitrary key/value labels for filtering and grouping. | [optional] 
**Name** | **string** | Human-readable, unique name (must match &#x60;^[a-z0-9][a-z0-9_-]{0,63}$&#x60;). | 
**Subnet** | Pointer to **NullableString** | IPv4/IPv6 subnet in CIDR notation (e.g. &#x60;\&quot;10.240.0.0/24\&quot;&#x60;). | [optional] 
**AttachedContainers** | [**[]BridgeNetworkAttachment**](BridgeNetworkAttachment.md) | Containers currently attached to the network. | 

## Methods

### NewBridgeNetworkDetails

`func NewBridgeNetworkDetails(createdAt time.Time, id string, name string, attachedContainers []BridgeNetworkAttachment, ) *BridgeNetworkDetails`

NewBridgeNetworkDetails instantiates a new BridgeNetworkDetails object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBridgeNetworkDetailsWithDefaults

`func NewBridgeNetworkDetailsWithDefaults() *BridgeNetworkDetails`

NewBridgeNetworkDetailsWithDefaults instantiates a new BridgeNetworkDetails object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *BridgeNetworkDetails) GetCreatedAt() time.Time`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *BridgeNetworkDetails) GetCreatedAtOk() (*time.Time, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *BridgeNetworkDetails) SetCreatedAt(v time.Time)`

SetCreatedAt sets CreatedAt field to given value.


### GetDriver

`func (o *BridgeNetworkDetails) GetDriver() BridgeNetworkDriver`

GetDriver returns the Driver field if non-nil, zero value otherwise.

### GetDriverOk

`func (o *BridgeNetworkDetails) GetDriverOk() (*BridgeNetworkDriver, bool)`

GetDriverOk returns a tuple with the Driver field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDriver

`func (o *BridgeNetworkDetails) SetDriver(v BridgeNetworkDriver)`

SetDriver sets Driver field to given value.

### HasDriver

`func (o *BridgeNetworkDetails) HasDriver() bool`

HasDriver returns a boolean if a field has been set.

### GetId

`func (o *BridgeNetworkDetails) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *BridgeNetworkDetails) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *BridgeNetworkDetails) SetId(v string)`

SetId sets Id field to given value.


### GetInternal

`func (o *BridgeNetworkDetails) GetInternal() bool`

GetInternal returns the Internal field if non-nil, zero value otherwise.

### GetInternalOk

`func (o *BridgeNetworkDetails) GetInternalOk() (*bool, bool)`

GetInternalOk returns a tuple with the Internal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetInternal

`func (o *BridgeNetworkDetails) SetInternal(v bool)`

SetInternal sets Internal field to given value.

### HasInternal

`func (o *BridgeNetworkDetails) HasInternal() bool`

HasInternal returns a boolean if a field has been set.

### GetLabels

`func (o *BridgeNetworkDetails) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *BridgeNetworkDetails) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *BridgeNetworkDetails) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *BridgeNetworkDetails) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetName

`func (o *BridgeNetworkDetails) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *BridgeNetworkDetails) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *BridgeNetworkDetails) SetName(v string)`

SetName sets Name field to given value.


### GetSubnet

`func (o *BridgeNetworkDetails) GetSubnet() string`

GetSubnet returns the Subnet field if non-nil, zero value otherwise.

### GetSubnetOk

`func (o *BridgeNetworkDetails) GetSubnetOk() (*string, bool)`

GetSubnetOk returns a tuple with the Subnet field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSubnet

`func (o *BridgeNetworkDetails) SetSubnet(v string)`

SetSubnet sets Subnet field to given value.

### HasSubnet

`func (o *BridgeNetworkDetails) HasSubnet() bool`

HasSubnet returns a boolean if a field has been set.

### SetSubnetNil

`func (o *BridgeNetworkDetails) SetSubnetNil(b bool)`

 SetSubnetNil sets the value for Subnet to be an explicit nil

### UnsetSubnet
`func (o *BridgeNetworkDetails) UnsetSubnet()`

UnsetSubnet ensures that no value is present for Subnet, not even an explicit nil
### GetAttachedContainers

`func (o *BridgeNetworkDetails) GetAttachedContainers() []BridgeNetworkAttachment`

GetAttachedContainers returns the AttachedContainers field if non-nil, zero value otherwise.

### GetAttachedContainersOk

`func (o *BridgeNetworkDetails) GetAttachedContainersOk() (*[]BridgeNetworkAttachment, bool)`

GetAttachedContainersOk returns a tuple with the AttachedContainers field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAttachedContainers

`func (o *BridgeNetworkDetails) SetAttachedContainers(v []BridgeNetworkAttachment)`

SetAttachedContainers sets AttachedContainers field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


