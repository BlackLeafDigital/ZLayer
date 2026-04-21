# TagImageRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Source** | **string** | Existing image reference to tag (e.g. &#x60;myapp:latest&#x60;). | 
**Target** | **string** | New reference to create (e.g. &#x60;registry.example.com/myapp:v1&#x60;). | 

## Methods

### NewTagImageRequest

`func NewTagImageRequest(source string, target string, ) *TagImageRequest`

NewTagImageRequest instantiates a new TagImageRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTagImageRequestWithDefaults

`func NewTagImageRequestWithDefaults() *TagImageRequest`

NewTagImageRequestWithDefaults instantiates a new TagImageRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetSource

`func (o *TagImageRequest) GetSource() string`

GetSource returns the Source field if non-nil, zero value otherwise.

### GetSourceOk

`func (o *TagImageRequest) GetSourceOk() (*string, bool)`

GetSourceOk returns a tuple with the Source field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSource

`func (o *TagImageRequest) SetSource(v string)`

SetSource sets Source field to given value.


### GetTarget

`func (o *TagImageRequest) GetTarget() string`

GetTarget returns the Target field if non-nil, zero value otherwise.

### GetTargetOk

`func (o *TagImageRequest) GetTargetOk() (*string, bool)`

GetTargetOk returns a tuple with the Target field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTarget

`func (o *TagImageRequest) SetTarget(v string)`

SetTarget sets Target field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


