# DaemonCapabilitiesResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CanWriteCgroupRoot** | **bool** | &#x60;true&#x60; if the cgroup root&#39;s &#x60;cgroup.subtree_control&#x60; has the owner- write bit set. | 
**CgroupParent** | Pointer to **NullableString** | The cgroup v2 path of the current process, if any. | [optional] 
**HasCapNetAdmin** | **bool** | &#x60;true&#x60; if &#x60;CAP_NET_ADMIN&#x60; is present in the process&#39;s bounding set. | 
**IsNested** | **bool** | &#x60;true&#x60; if the process appears to be inside a container (non-root cgroup v2 path). | 
**IsRoot** | **bool** | &#x60;true&#x60; if the process is running as uid 0. | 
**Mode** | [**DaemonModeDto**](DaemonModeDto.md) | Coarse classification derived from the boolean fields below. | 
**TunDeviceAvailable** | **bool** | &#x60;true&#x60; if &#x60;/dev/net/tun&#x60; can be opened r/w without EACCES/EPERM/ENOENT/ENXIO. | 

## Methods

### NewDaemonCapabilitiesResponse

`func NewDaemonCapabilitiesResponse(canWriteCgroupRoot bool, hasCapNetAdmin bool, isNested bool, isRoot bool, mode DaemonModeDto, tunDeviceAvailable bool, ) *DaemonCapabilitiesResponse`

NewDaemonCapabilitiesResponse instantiates a new DaemonCapabilitiesResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDaemonCapabilitiesResponseWithDefaults

`func NewDaemonCapabilitiesResponseWithDefaults() *DaemonCapabilitiesResponse`

NewDaemonCapabilitiesResponseWithDefaults instantiates a new DaemonCapabilitiesResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCanWriteCgroupRoot

`func (o *DaemonCapabilitiesResponse) GetCanWriteCgroupRoot() bool`

GetCanWriteCgroupRoot returns the CanWriteCgroupRoot field if non-nil, zero value otherwise.

### GetCanWriteCgroupRootOk

`func (o *DaemonCapabilitiesResponse) GetCanWriteCgroupRootOk() (*bool, bool)`

GetCanWriteCgroupRootOk returns a tuple with the CanWriteCgroupRoot field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCanWriteCgroupRoot

`func (o *DaemonCapabilitiesResponse) SetCanWriteCgroupRoot(v bool)`

SetCanWriteCgroupRoot sets CanWriteCgroupRoot field to given value.


### GetCgroupParent

`func (o *DaemonCapabilitiesResponse) GetCgroupParent() string`

GetCgroupParent returns the CgroupParent field if non-nil, zero value otherwise.

### GetCgroupParentOk

`func (o *DaemonCapabilitiesResponse) GetCgroupParentOk() (*string, bool)`

GetCgroupParentOk returns a tuple with the CgroupParent field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCgroupParent

`func (o *DaemonCapabilitiesResponse) SetCgroupParent(v string)`

SetCgroupParent sets CgroupParent field to given value.

### HasCgroupParent

`func (o *DaemonCapabilitiesResponse) HasCgroupParent() bool`

HasCgroupParent returns a boolean if a field has been set.

### SetCgroupParentNil

`func (o *DaemonCapabilitiesResponse) SetCgroupParentNil(b bool)`

 SetCgroupParentNil sets the value for CgroupParent to be an explicit nil

### UnsetCgroupParent
`func (o *DaemonCapabilitiesResponse) UnsetCgroupParent()`

UnsetCgroupParent ensures that no value is present for CgroupParent, not even an explicit nil
### GetHasCapNetAdmin

`func (o *DaemonCapabilitiesResponse) GetHasCapNetAdmin() bool`

GetHasCapNetAdmin returns the HasCapNetAdmin field if non-nil, zero value otherwise.

### GetHasCapNetAdminOk

`func (o *DaemonCapabilitiesResponse) GetHasCapNetAdminOk() (*bool, bool)`

GetHasCapNetAdminOk returns a tuple with the HasCapNetAdmin field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHasCapNetAdmin

`func (o *DaemonCapabilitiesResponse) SetHasCapNetAdmin(v bool)`

SetHasCapNetAdmin sets HasCapNetAdmin field to given value.


### GetIsNested

`func (o *DaemonCapabilitiesResponse) GetIsNested() bool`

GetIsNested returns the IsNested field if non-nil, zero value otherwise.

### GetIsNestedOk

`func (o *DaemonCapabilitiesResponse) GetIsNestedOk() (*bool, bool)`

GetIsNestedOk returns a tuple with the IsNested field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIsNested

`func (o *DaemonCapabilitiesResponse) SetIsNested(v bool)`

SetIsNested sets IsNested field to given value.


### GetIsRoot

`func (o *DaemonCapabilitiesResponse) GetIsRoot() bool`

GetIsRoot returns the IsRoot field if non-nil, zero value otherwise.

### GetIsRootOk

`func (o *DaemonCapabilitiesResponse) GetIsRootOk() (*bool, bool)`

GetIsRootOk returns a tuple with the IsRoot field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIsRoot

`func (o *DaemonCapabilitiesResponse) SetIsRoot(v bool)`

SetIsRoot sets IsRoot field to given value.


### GetMode

`func (o *DaemonCapabilitiesResponse) GetMode() DaemonModeDto`

GetMode returns the Mode field if non-nil, zero value otherwise.

### GetModeOk

`func (o *DaemonCapabilitiesResponse) GetModeOk() (*DaemonModeDto, bool)`

GetModeOk returns a tuple with the Mode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMode

`func (o *DaemonCapabilitiesResponse) SetMode(v DaemonModeDto)`

SetMode sets Mode field to given value.


### GetTunDeviceAvailable

`func (o *DaemonCapabilitiesResponse) GetTunDeviceAvailable() bool`

GetTunDeviceAvailable returns the TunDeviceAvailable field if non-nil, zero value otherwise.

### GetTunDeviceAvailableOk

`func (o *DaemonCapabilitiesResponse) GetTunDeviceAvailableOk() (*bool, bool)`

GetTunDeviceAvailableOk returns a tuple with the TunDeviceAvailable field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTunDeviceAvailable

`func (o *DaemonCapabilitiesResponse) SetTunDeviceAvailable(v bool)`

SetTunDeviceAvailable sets TunDeviceAvailable field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


